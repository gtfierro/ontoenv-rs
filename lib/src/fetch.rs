//! Facilities for retrieving remote RDF graphs and ontologies.
//!
//! The helpers in this module implement conservative content negotiation, inspect
//! HTTP `Link` headers for format-specific alternates, try common file-extension
//! rewrites, and finally fall back to light-weight content sniffing. Together these
//! heuristics allow OntoEnv to download RDF resources from a wide range of servers
//! that may not advertise perfect metadata.

use crate::errors::OfflineRetrievalError;
use anyhow::{anyhow, Result};
use chrono::prelude::*;
use oxigraph::io::RdfFormat;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE, LINK};
use reqwest::Url;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::time::Duration;

/// Options that control how remote RDF resources are fetched.
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// Fail immediately when `true`; callers use this to guard offline modes.
    pub offline: bool,
    /// Overall network timeout applied to individual HTTP requests.
    pub timeout: Duration,
    /// Ordered list of media types to negotiate, highest priority first.
    pub accept_order: Vec<&'static str>,
    /// Paths that are appended to the source URL when probing alternate locations.
    pub extension_candidates: Vec<&'static str>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        const DEFAULT_ACCEPT: &[&str] = &[
            "text/turtle",
            "application/ld+json",
            "application/n-quads",
            "application/trig",
            "application/rdf+xml",
            "application/n-triples",
            "text/n3",
            "application/owl+xml",
            "application/xml",
        ];
        const DEFAULT_EXTENSION_CANDIDATES: &[&str] = &[
            ".ttl",
            ".rdf",
            ".owl",
            ".rdf.xml",
            ".owl.xml",
            ".xml",
            ".jsonld",
            ".nq",
            ".nt",
            "index.ttl",
            "index.rdf",
            "index.rdf.xml",
            "index.owl.xml",
            "index.xml",
            "index.jsonld",
        ];
        Self {
            offline: false,
            timeout: Duration::from_secs(30),
            accept_order: DEFAULT_ACCEPT.to_vec(),
            extension_candidates: DEFAULT_EXTENSION_CANDIDATES.to_vec(),
        }
    }
}

/// Successful network fetch including bytes and detected format metadata.
#[derive(Debug, Clone)]
pub struct FetchResult {
    pub bytes: Vec<u8>,
    pub format: Option<RdfFormat>,
    pub final_url: String,
    pub content_type: Option<String>,
}

/// Attempts to identify an RDF serialization from the supplied media type.
fn detect_format(ct: &str) -> Option<RdfFormat> {
    RdfFormat::from_media_type(ct.trim())
}

/// Builds a weighted `Accept` header string honoring the provided priority order.
fn build_accept(accept_order: &[&'static str]) -> String {
    if accept_order.is_empty() {
        return "*/*".to_string();
    }
    let mut parts = Vec::new();
    let mut q = 1.0f32;
    for (i, t) in accept_order.iter().enumerate() {
        parts.push(format!("{t}; q={:.2}", q));
        let next = (q - 0.1f32).max(0.1f32);
        q = if i + 1 == accept_order.len() {
            0.1
        } else {
            next
        };
    }
    parts.push("application/octet-stream; q=0.1".to_string());
    parts.push("*/*; q=0.05".to_string());
    parts.join(", ")
}

/// Generates a list of alternate URLs to try by swapping or appending common RDF extensions.
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
        for rep in [
            ".ttl", ".rdf", ".owl", ".rdf.xml", ".owl.xml", ".xml", ".jsonld", ".nq", ".nt",
        ] {
            cands.push(format!("{base}{rep}"));
        }
    } else {
        for rep in [
            ".ttl", ".rdf", ".owl", ".rdf.xml", ".owl.xml", ".xml", ".jsonld", ".nq", ".nt",
        ] {
            cands.push(format!("{orig}{rep}"));
        }
    }
    cands
}

/// Extracts `rel="alternate"` RDF targets from an HTTP `Link` header.
fn parse_link_alternates(headers: &HeaderMap, accept_order: &[&'static str]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(link_val) = headers.get(LINK) {
        if let Ok(link_str) = link_val.to_str() {
            for part in link_str.split(',') {
                let part = part.trim();
                let part_lower = part.to_ascii_lowercase();
                if !part_lower.contains("rel=\"alternate\"")
                    && !part_lower.contains("rel='alternate'")
                {
                    continue;
                }
                let has_rdf_type = accept_order
                    .iter()
                    .map(|typ| typ.to_ascii_lowercase())
                    .any(|typ| part_lower.contains(&typ));
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
) -> Result<(
    Vec<u8>,
    Option<String>,
    Option<String>,
    String,
    reqwest::StatusCode,
)> {
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

/// Attempts to infer an RDF format from the URL path extension.
fn detect_format_from_url(url: &str) -> Option<RdfFormat> {
    let trimmed = url.split('#').next().unwrap_or(url);
    let path = trimmed.split('?').next().unwrap_or(trimmed);
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(RdfFormat::from_extension)
}

/// Provides a last-resort guess at the RDF serialization by peeking at the payload.
fn sniff_format(bytes: &[u8]) -> Option<RdfFormat> {
    let sample_len = bytes.len().min(4096);
    let sample = std::str::from_utf8(&bytes[..sample_len]).ok()?;
    let trimmed = sample.trim_start();

    if trimmed.starts_with('{') && sample.contains("\"@context\"") {
        return detect_format("application/ld+json");
    }
    if trimmed.starts_with('<') {
        if sample.contains("<rdf:RDF") || sample.contains("xmlns:rdf") {
            return detect_format("application/rdf+xml");
        }
        if sample.contains("<Ontology") || sample.contains("<owl:") {
            return detect_format("application/rdf+xml");
        }
    }
    if sample.contains("@prefix") || sample.contains("@base") || sample.contains("PREFIX ") {
        return detect_format("text/turtle");
    }
    if sample.contains("GRAPH") && sample.contains('{') {
        return detect_format("application/trig");
    }
    if sample.contains("\n_:") {
        return detect_format("application/n-triples");
    }
    None
}

/// Returns `true` when the response appears to be HTML instead of RDF.
fn looks_like_html(content_type: Option<&str>, bytes: &[u8]) -> bool {
    if let Some(ct) = content_type {
        let lc = ct.to_ascii_lowercase();
        if lc.contains("text/html") || lc.contains("application/xhtml") {
            return true;
        }
    }
    let prefix_len = bytes.len().min(512);
    if let Ok(snippet) = std::str::from_utf8(&bytes[..prefix_len]) {
        let lower = snippet.to_ascii_lowercase();
        return lower.contains("<html") || lower.contains("<!doctype html");
    }
    false
}

/// Builds a [`FetchResult`] applying media-type, extension, and content sniffing heuristics.
fn build_result(bytes: Vec<u8>, ct: Option<String>, final_url: String) -> FetchResult {
    let mut format = ct.as_deref().and_then(detect_format);
    if format.is_none() {
        format = detect_format_from_url(&final_url);
    }
    if format.is_none() {
        format = sniff_format(&bytes);
    }
    FetchResult {
        bytes,
        format,
        final_url,
        content_type: ct,
    }
}

/// Searches HTML bodies for `<link rel="alternate">` elements that advertise RDF derivatives.
fn parse_html_alternates(html: &str, accept_order: &[&'static str]) -> Vec<String> {
    let mut out = Vec::new();
    let lower_html = html.to_ascii_lowercase();
    let mut idx = 0usize;
    while let Some(rel_pos) = lower_html[idx..].find("<link") {
        let start = idx + rel_pos;
        let remainder = &lower_html[start..];
        let Some(close_rel) = remainder.find('>') else {
            break;
        };
        let end = start + close_rel + 1;
        let tag_lower = &lower_html[start..end];
        if !(tag_lower.contains("rel=\"alternate\"") || tag_lower.contains("rel='alternate'")) {
            idx = end;
            continue;
        }
        let mut type_match = false;
        for typ in accept_order {
            let typ_lower = typ.to_ascii_lowercase();
            if tag_lower.contains(&format!("type=\"{}\"", typ_lower))
                || tag_lower.contains(&format!("type='{}'", typ_lower))
            {
                type_match = true;
                break;
            }
        }
        if !type_match {
            idx = end;
            continue;
        }
        let tag_original = &html[start..end];
        if let Some(href) = extract_href(tag_original, tag_lower) {
            out.push(href.to_string());
        }
        idx = end;
    }
    out
}

/// Extracts the value of the `href` attribute from a link tag, preserving original casing.
fn extract_href<'a>(tag_original: &'a str, tag_lower: &str) -> Option<&'a str> {
    for (pattern, delim) in [("href=\"", '"'), ("href='", '\'')] {
        if let Some(idx) = tag_lower.find(pattern) {
            let start = idx + pattern.len();
            if let Some(end_rel) = tag_lower[start..].find(delim) {
                let end = start + end_rel;
                return Some(&tag_original[start..end]);
            }
        }
    }
    None
}

fn resolve_relative(base: &str, candidate: &str) -> String {
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        return candidate.to_string();
    }
    match Url::parse(base) {
        Ok(base_url) => base_url
            .join(candidate)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| candidate.to_string()),
        Err(_) => candidate.to_string(),
    }
}

/// Fetches RDF content from the provided `url`, applying layered fallbacks when
/// servers respond with HTML or ambiguous metadata. The strategy is:
///
/// 1. Send a single request with a weighted `Accept` header covering common RDF media types.
/// 2. If that fails to yield a recognizable format, re-try discovered alternates from HTTP `Link`
///    headers, HTML `<link rel="alternate">` elements, and well-known file-extension rewrites.
/// 3. For stubborn endpoints, re-issue requests with one MIME type at a time to coax content
///    negotiation, and finally fall back to format sniffing when a body looks like RDF despite
///    missing metadata.
pub fn fetch_rdf(url: &str, opts: &FetchOptions) -> Result<FetchResult> {
    if opts.offline {
        return Err(anyhow!(OfflineRetrievalError {
            file: url.to_string()
        }));
    }
    let client = Client::builder().timeout(opts.timeout).build()?;
    let default_accept = build_accept(&opts.accept_order);

    let mut queue: VecDeque<(String, String)> = VecDeque::new();
    queue.push_back((url.to_string(), default_accept.clone()));
    let mut visited: HashSet<(String, String)> = HashSet::new();
    let mut last_error: Option<anyhow::Error> = None;
    let mut best_success: Option<FetchResult> = None;

    while let Some((candidate_url, accept_header)) = queue.pop_front() {
        if !visited.insert((candidate_url.clone(), accept_header.clone())) {
            continue;
        }

        let response = try_get(&candidate_url, &client, &accept_header);
        let Ok((bytes, ct, link, final_url, status)) = response else {
            if let Err(err) = response {
                if last_error.is_none() {
                    last_error = Some(err);
                }
            }
            continue;
        };

        let result = build_result(bytes, ct.clone(), final_url.clone());
        let is_html = looks_like_html(result.content_type.as_deref(), &result.bytes);

        if status.is_success() {
            match &best_success {
                None => best_success = Some(result.clone()),
                Some(existing) if existing.format.is_none() && result.format.is_some() => {
                    best_success = Some(result.clone())
                }
                _ => (),
            }

            if result.format.is_some() && !is_html {
                return Ok(result);
            }
        }

        if let Some(link_header) = link {
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&link_header) {
                headers.insert(LINK, value);
            }
            for alt in parse_link_alternates(&headers, &opts.accept_order) {
                let resolved = resolve_relative(&final_url, &alt);
                queue.push_back((resolved, default_accept.clone()));
            }
        }

        if is_html {
            if let Ok(body) = std::str::from_utf8(&result.bytes) {
                for alt in parse_html_alternates(body, &opts.accept_order) {
                    let resolved = resolve_relative(&final_url, &alt);
                    queue.push_back((resolved, default_accept.clone()));
                }
            }
        }

        if !status.is_success() || is_html || result.format.is_none() {
            for candidate in build_extension_candidates(&final_url, &opts.extension_candidates) {
                queue.push_back((candidate, default_accept.clone()));
            }
            for accept in &opts.accept_order {
                queue.push_back((final_url.clone(), accept.to_string()));
            }
        }
    }

    if let Some(success) = best_success {
        return Ok(success);
    }

    Err(last_error.unwrap_or_else(|| {
        anyhow!("Failed to retrieve RDF from {url} using available negotiation strategies")
    }))
}

/// Issues a `HEAD` request and returns the parsed `Last-Modified` timestamp, when present.
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

/// Returns `true` if the remote resource answers successfully to a `HEAD` probe.
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
