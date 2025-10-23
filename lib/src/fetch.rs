use crate::errors::OfflineRetrievalError;
use anyhow::{anyhow, Result};
use chrono::prelude::*;
use oxigraph::io::{JsonLdProfileSet, RdfFormat, RdfParser};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE, LINK};
use std::io::Cursor;
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
                "application/ld+json",
                "application/n-triples",
            ],
            extension_candidates: vec![
                ".ttl",
                ".rdf",
                ".owl",
                ".rdf.xml",
                ".owl.xml",
                ".xml",
                ".jsonld",
                ".nt",
                ".nq",
                "index.ttl",
                "index.rdf",
                "index.rdf.xml",
                "index.owl.xml",
                "index.xml",
                "index.jsonld",
            ],
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
    let ct = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match ct.as_str() {
        "text/turtle" | "application/x-turtle" => Some(RdfFormat::Turtle),
        "application/rdf+xml" => Some(RdfFormat::RdfXml),
        "application/n-triples" | "application/ntriples" | "text/plain" => {
            Some(RdfFormat::NTriples)
        }
        _ => None,
    }
}

fn detect_format_from_url(url: &str) -> Option<RdfFormat> {
    let trimmed = url.split('#').next().unwrap_or(url);
    let path = trimmed.split('?').next().unwrap_or(trimmed);
    std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .and_then(|ext| match ext.as_str() {
            "ttl" => Some(RdfFormat::Turtle),
            "rdf" | "owl" | "xml" => Some(RdfFormat::RdfXml),
            "nt" => Some(RdfFormat::NTriples),
            "jsonld" | "json" => Some(RdfFormat::JsonLd {
                profile: JsonLdProfileSet::default(),
            }),
            "nq" | "trig" => Some(RdfFormat::NQuads),
            _ => None,
        })
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
        for rep in [
            ".ttl", ".rdf", ".owl", ".rdf.xml", ".owl.xml", ".xml", ".jsonld", ".nt", ".nq",
        ] {
            cands.push(format!("{base}{rep}"));
        }
    } else {
        for rep in [
            ".ttl", ".rdf", ".owl", ".rdf.xml", ".owl.xml", ".xml", ".jsonld", ".nt", ".nq",
        ] {
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

fn sniff_format(bytes: &[u8]) -> Option<RdfFormat> {
    let sample_len = bytes.len().min(4096);
    let sample = std::str::from_utf8(&bytes[..sample_len]).ok()?;
    let trimmed = sample.trim_start();

    if trimmed.starts_with('{') && sample.contains("\"@context\"") {
        return Some(RdfFormat::JsonLd {
            profile: JsonLdProfileSet::default(),
        });
    }
    if trimmed.starts_with('<') {
        if sample.contains("<rdf:RDF") || sample.contains("xmlns:rdf") {
            return Some(RdfFormat::RdfXml);
        }
        if sample.contains("<Ontology") || sample.contains("<owl:") {
            return Some(RdfFormat::RdfXml);
        }
    }
    if sample.contains("@prefix") || sample.contains("@base") || sample.contains("PREFIX ") {
        return Some(RdfFormat::Turtle);
    }
    if sample.contains("GRAPH") && sample.contains('{') {
        return Some(RdfFormat::TriG);
    }
    if sample.contains("\n_:") {
        return Some(RdfFormat::NTriples);
    }
    None
}

fn can_parse_as(bytes: &[u8], format: RdfFormat) -> bool {
    let cursor = Cursor::new(bytes);
    let parser = RdfParser::from_format(format);
    let mut reader = parser.for_reader(cursor);
    while let Some(result) = reader.next() {
        match result {
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
    true
}

fn try_parse_candidates(bytes: &[u8]) -> Option<RdfFormat> {
    let candidates = [
        RdfFormat::Turtle,
        RdfFormat::RdfXml,
        RdfFormat::NTriples,
        RdfFormat::NQuads,
        RdfFormat::TriG,
        RdfFormat::JsonLd {
            profile: JsonLdProfileSet::default(),
        },
    ];
    for fmt in candidates {
        if can_parse_as(bytes, fmt) {
            return Some(fmt);
        }
    }
    None
}

fn is_generic_content_type(ct: Option<&str>) -> bool {
    match ct.map(|s| s.to_ascii_lowercase()) {
        None => true,
        Some(ref s) if s.contains("text/plain") => true,
        Some(ref s) if s.contains("application/octet-stream") => true,
        Some(ref s) if s.contains("text/html") => true,
        Some(ref s) if s.contains("application/xhtml") => true,
        _ => false,
    }
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
    let mut content_type = ct.clone();

    // Best-effort extra HEAD probe to refine content type if needed
    if is_generic_content_type(content_type.as_deref()) {
        if let Ok(resp) = client.head(&final_url).header(ACCEPT, &accept).send() {
            if resp.status().is_success() {
                if let Some(ct_head) = resp
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|h| h.to_str().ok())
                {
                    content_type = Some(ct_head.to_string());
                }
            }
        }
    }

    // If success evaluate heuristics
    if status.is_success() {
        if let Some(fmt) = content_type
            .as_deref()
            .and_then(detect_format)
            .or_else(|| detect_format_from_url(&final_url))
            .or_else(|| sniff_format(&bytes))
        {
            return Ok(FetchResult {
                bytes,
                format: Some(fmt),
                final_url,
                content_type,
            });
        }

        if let Some(fmt) = try_parse_candidates(&bytes) {
            return Ok(FetchResult {
                bytes,
                format: Some(fmt),
                final_url,
                content_type,
            });
        }
    }

    // Try Link: rel="alternate" with single pass
    if let Some(link_header) = link {
        let mut headers = HeaderMap::new();
        headers.insert(
            LINK,
            HeaderValue::from_str(&link_header).unwrap_or(HeaderValue::from_static("")),
        );
        for alt in parse_link_alternates(&headers, &opts.accept_order) {
            let (b2, ct2, _link2, fu2, st2) = try_get(&alt, &client, &accept)?;
            if st2.is_success() {
                let guess = ct2
                    .as_deref()
                    .and_then(detect_format)
                    .or_else(|| detect_format_from_url(&fu2))
                    .or_else(|| sniff_format(&b2))
                    .or_else(|| try_parse_candidates(&b2));
                if let Some(fmt) = guess {
                    return Ok(FetchResult {
                        bytes: b2,
                        format: Some(fmt),
                        final_url: fu2,
                        content_type: ct2,
                    });
                }
            }
        }
    }

    // Status-based or type-based fallbacks
    if !status.is_success() || is_generic_content_type(content_type.as_deref()) {
        for candidate in build_extension_candidates(&final_url, &opts.extension_candidates) {
            let (b2, ct2, _link2, fu2, st2) = try_get(&candidate, &client, &accept)?;
            if st2.is_success() {
                let guess = ct2
                    .as_deref()
                    .and_then(detect_format)
                    .or_else(|| detect_format_from_url(&fu2))
                    .or_else(|| sniff_format(&b2))
                    .or_else(|| try_parse_candidates(&b2));
                if let Some(fmt) = guess {
                    return Ok(FetchResult {
                        bytes: b2,
                        format: Some(fmt),
                        final_url: fu2,
                        content_type: ct2,
                    });
                }
            }
        }
    }

    if status.is_success() {
        let fmt = content_type
            .as_deref()
            .and_then(detect_format)
            .or_else(|| detect_format_from_url(&final_url))
            .or_else(|| sniff_format(&bytes))
            .or_else(|| try_parse_candidates(&bytes));
        return Ok(FetchResult {
            bytes,
            format: fmt,
            final_url,
            content_type,
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
