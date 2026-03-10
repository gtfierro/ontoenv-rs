use std::error::Error;

use clap::{Parser, ValueEnum};
use rdf5d::Term;

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum Scenario {
    RepeatedLang,
    RepeatedDatatype,
    MixedReuse,
    UniqueLiterals,
}

#[derive(Debug, Parser)]
#[command(name = "literal_component_profile")]
#[command(about = "Compare inline vs component-backed literal TERM_DICT sizing")]
struct Args {
    #[arg(long, value_enum, default_value_t = Scenario::RepeatedLang)]
    scenario: Scenario,
    #[arg(long, default_value_t = 10_000)]
    terms: usize,
}

#[derive(Debug)]
struct SizeReport {
    scenario: &'static str,
    terms: usize,
    inline_term_dict_bytes: u64,
    component_term_dict_bytes: u64,
    delta_bytes: i64,
    savings_pct: f64,
    chosen_mode: &'static str,
}

#[derive(Debug, Default)]
struct DictPool {
    values: Vec<String>,
}

fn push_uvarint_len(mut v: u64) -> u64 {
    let mut len = 1u64;
    while v >= 0x80 {
        v >>= 7;
        len += 1;
    }
    len
}

fn component_dict_bytes(strings: &[String]) -> u64 {
    let blob = strings.iter().map(|s| s.len() as u64).sum::<u64>();
    20 + blob + ((strings.len() as u64 + 1) * 4)
}

fn intern(pool: &mut DictPool, value: &str) -> u32 {
    if let Some(idx) = pool.values.iter().position(|existing| existing == value) {
        idx as u32
    } else {
        let idx = pool.values.len() as u32;
        pool.values.push(value.to_string());
        idx
    }
}

fn term_dict_inline_bytes(terms: &[Term]) -> u64 {
    let mut payload = 0u64;
    for term in terms {
        match term {
            Term::Iri(s) | Term::BNode(s) => payload += s.len() as u64,
            Term::Literal { lex, dt, lang } => {
                payload += push_uvarint_len(lex.len() as u64) + lex.len() as u64 + 2;
                if let Some(dt) = dt {
                    payload += push_uvarint_len(dt.len() as u64) + dt.len() as u64;
                }
                if let Some(lang) = lang {
                    payload += push_uvarint_len(lang.len() as u64) + lang.len() as u64;
                }
            }
        }
    }
    33 + terms.len() as u64 + payload + ((terms.len() as u64 + 1) * 4)
}

fn term_dict_component_bytes(terms: &[Term]) -> u64 {
    let mut lex = DictPool::default();
    let mut dt = DictPool::default();
    let mut lang = DictPool::default();
    let mut payload = 0u64;

    for term in terms {
        match term {
            Term::Iri(s) | Term::BNode(s) => payload += s.len() as u64,
            Term::Literal {
                lex: l,
                dt: d,
                lang: g,
            } => {
                let lex_id = intern(&mut lex, l);
                payload += push_uvarint_len(lex_id as u64);
                payload += if let Some(dt_value) = d {
                    let dt_id = intern(&mut dt, dt_value);
                    push_uvarint_len(dt_id as u64 + 1)
                } else {
                    1
                };
                payload += if let Some(lang_value) = g {
                    let lang_id = intern(&mut lang, lang_value);
                    push_uvarint_len(lang_id as u64 + 1)
                } else {
                    1
                };
            }
        }
    }

    33 + terms.len() as u64
        + component_dict_bytes(&lex.values)
        + component_dict_bytes(&dt.values)
        + component_dict_bytes(&lang.values)
        + payload
        + ((terms.len() as u64 + 1) * 4)
}

fn scenario_name(scenario: Scenario) -> &'static str {
    match scenario {
        Scenario::RepeatedLang => "repeated_lang",
        Scenario::RepeatedDatatype => "repeated_datatype",
        Scenario::MixedReuse => "mixed_reuse",
        Scenario::UniqueLiterals => "unique_literals",
    }
}

fn generate_terms(scenario: Scenario, n: usize) -> Vec<Term> {
    let mut terms = Vec::with_capacity(n);
    for idx in 0..n {
        let term = match scenario {
            Scenario::RepeatedLang => Term::Literal {
                lex: format!("label-{}", idx % 20),
                dt: None,
                lang: Some(format!("lang-{}", idx % 5)),
            },
            Scenario::RepeatedDatatype => Term::Literal {
                lex: format!("value-{}", idx % 100),
                dt: Some("http://www.w3.org/2001/XMLSchema#string".into()),
                lang: None,
            },
            Scenario::MixedReuse => {
                if idx % 2 == 0 {
                    Term::Literal {
                        lex: format!("mixed-{}", idx % 50),
                        dt: Some("http://www.w3.org/2001/XMLSchema#integer".into()),
                        lang: None,
                    }
                } else {
                    Term::Literal {
                        lex: format!("mixed-{}", idx % 50),
                        dt: None,
                        lang: Some(format!("lang-{}", idx % 4)),
                    }
                }
            }
            Scenario::UniqueLiterals => Term::Literal {
                lex: format!("unique-{}-{}", idx, "x".repeat(16)),
                dt: Some(format!("http://example.org/dt/{}", idx)),
                lang: Some(format!("lang-{}", idx)),
            },
        };
        terms.push(term);
    }
    terms
}

fn build_report(scenario: Scenario, terms: usize) -> SizeReport {
    let term_values = generate_terms(scenario, terms);
    let inline = term_dict_inline_bytes(&term_values);
    let component = term_dict_component_bytes(&term_values);
    let delta = inline as i64 - component as i64;
    let chosen_mode = if component < inline {
        "component"
    } else {
        "inline"
    };
    SizeReport {
        scenario: scenario_name(scenario),
        terms,
        inline_term_dict_bytes: inline,
        component_term_dict_bytes: component,
        delta_bytes: delta,
        savings_pct: if inline == 0 {
            0.0
        } else {
            (delta as f64 / inline as f64) * 100.0
        },
        chosen_mode,
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let report = build_report(args.scenario, args.terms);
    println!("{{");
    println!("  \"scenario\": \"{}\",", report.scenario);
    println!("  \"terms\": {},", report.terms);
    println!(
        "  \"inline_term_dict_bytes\": {},",
        report.inline_term_dict_bytes
    );
    println!(
        "  \"component_term_dict_bytes\": {},",
        report.component_term_dict_bytes
    );
    println!("  \"delta_bytes\": {},", report.delta_bytes);
    println!("  \"savings_pct\": {:.2},", report.savings_pct);
    println!("  \"chosen_mode\": \"{}\"", report.chosen_mode);
    println!("}}");
    Ok(())
}
