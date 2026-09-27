use davr_types::{ContextMetricsRecord, TokenUsageRecord};
use regex::Regex;
use std::sync::OnceLock;

static RE_TOKENS_IN_OUT: OnceLock<Regex> = OnceLock::new();
static RE_PROMPT_COMPLETION: OnceLock<Regex> = OnceLock::new();
static RE_TOTAL_TOKENS: OnceLock<Regex> = OnceLock::new();
static RE_COST: OnceLock<Regex> = OnceLock::new();
static RE_CONTEXT_PERCENT: OnceLock<Regex> = OnceLock::new();
static RE_CONTEXT_RATIO: OnceLock<Regex> = OnceLock::new();

fn tokens_in_out_regex() -> &'static Regex {
    RE_TOKENS_IN_OUT.get_or_init(|| {
        Regex::new(r"(?i)\b(?:tokens?|usage):\s*([0-9.,km]+)\s*(?:in|input|sent)\b(?:[,\s]+([0-9.,km]+)\s*(?:out|output|received)\b)?")
            .expect("Valid regex")
    })
}

fn prompt_completion_regex() -> &'static Regex {
    RE_PROMPT_COMPLETION.get_or_init(|| {
        Regex::new(r"(?i)(?:prompt\s+tokens?|input\s+tokens?):\s*([0-9.,km]+)(?:[,\s]+(?:completion\s+tokens?|output\s+tokens?):\s*([0-9.,km]+))?")
            .expect("Valid regex")
    })
}

fn total_tokens_regex() -> &'static Regex {
    RE_TOTAL_TOKENS.get_or_init(|| {
        Regex::new(r"(?i)\btotal(?:\s+tokens?)?:\s*([0-9.,km]+)\b").expect("Valid regex")
    })
}

fn cost_regex() -> &'static Regex {
    RE_COST.get_or_init(|| Regex::new(r"(?i)\bcost:\s*\$([0-9.]+)\b").expect("Valid regex"))
}

fn context_percent_regex() -> &'static Regex {
    RE_CONTEXT_PERCENT.get_or_init(|| {
        Regex::new(r"(?i)context(?:\s+window)?(?:\s+(?:is|at|fill|usage|limit))?[:\s]+([0-9.]+)%")
            .expect("Valid regex")
    })
}

fn context_ratio_regex() -> &'static Regex {
    RE_CONTEXT_RATIO.get_or_init(|| {
        Regex::new(r"(?i)context\s+fill\s+ratio:\s*([0-9.]+)").expect("Valid regex")
    })
}

/// Parses number string with potential comma separators and 'k' / 'm' suffixes
fn parse_number(s: &str) -> Option<i64> {
    let clean = s.trim().replace(',', "").to_lowercase();
    if let Some(rest) = clean.strip_suffix('k') {
        let n: f64 = rest.parse().ok()?;
        Some((n * 1000.0).round() as i64)
    } else if let Some(rest) = clean.strip_suffix('m') {
        let n: f64 = rest.parse().ok()?;
        Some((n * 1_000_000.0).round() as i64)
    } else {
        clean.parse::<i64>().ok()
    }
}

/// Strips standard ANSI terminal escape sequences
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub struct TokenScraper;

impl TokenScraper {
    /// Inspects a line of stdout or stderr, returning extracted token and/or context metrics
    pub fn parse_line(raw_line: &str) -> (Option<TokenUsageRecord>, Option<ContextMetricsRecord>) {
        let line = strip_ansi(raw_line);
        let mut token_record: Option<TokenUsageRecord> = None;
        let mut context_record: Option<ContextMetricsRecord> = None;

        // 1. Try structured JSON pattern
        if line.contains('{') && (line.contains("tokens") || line.contains("prompt_")) {
            if let Some(start) = line.find('{') {
                if let Some(end) = line.rfind('}') {
                    if end > start {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line[start..=end])
                        {
                            let inp = v
                                .get("prompt_tokens")
                                .or_else(|| v.get("input_tokens"))
                                .and_then(|x| x.as_i64());
                            let out = v
                                .get("completion_tokens")
                                .or_else(|| v.get("output_tokens"))
                                .and_then(|x| x.as_i64());
                            let cached = v.get("cached_tokens").and_then(|x| x.as_i64());
                            let mut tot = v.get("total_tokens").and_then(|x| x.as_i64());
                            if tot.is_none() && (inp.is_some() || out.is_some()) {
                                tot = Some(inp.unwrap_or(0) + out.unwrap_or(0));
                            }
                            let cost = v
                                .get("cost")
                                .or_else(|| v.get("cost_usd"))
                                .and_then(|x| x.as_f64());

                            if inp.is_some() || out.is_some() || tot.is_some() {
                                token_record = Some(TokenUsageRecord {
                                    input_tokens: inp,
                                    output_tokens: out,
                                    cached_tokens: cached,
                                    total_tokens: tot,
                                    cost_usd: cost,
                                });
                            }
                        }
                    }
                }
            }
        }

        // 2. Try CLI pattern: "Tokens: X in, Y out" or "Tokens: X sent, Y received"
        if token_record.is_none() {
            if let Some(caps) = tokens_in_out_regex().captures(&line) {
                let inp = caps.get(1).and_then(|m| parse_number(m.as_str()));
                let out = caps.get(2).and_then(|m| parse_number(m.as_str()));
                let total = match (inp, out) {
                    (Some(i), Some(o)) => Some(i + o),
                    (Some(i), None) => Some(i),
                    _ => None,
                };

                let cost = cost_regex()
                    .captures(&line)
                    .and_then(|c| c.get(1))
                    .and_then(|m| m.as_str().parse::<f64>().ok());

                if inp.is_some() || out.is_some() {
                    token_record = Some(TokenUsageRecord {
                        input_tokens: inp,
                        output_tokens: out,
                        cached_tokens: None,
                        total_tokens: total,
                        cost_usd: cost,
                    });
                }
            }
        }

        // 3. Try CLI pattern: "Prompt tokens: X, Completion tokens: Y"
        if token_record.is_none() {
            if let Some(caps) = prompt_completion_regex().captures(&line) {
                let inp = caps.get(1).and_then(|m| parse_number(m.as_str()));
                let out = caps.get(2).and_then(|m| parse_number(m.as_str()));
                let mut total = match (inp, out) {
                    (Some(i), Some(o)) => Some(i + o),
                    (Some(i), None) => Some(i),
                    _ => None,
                };

                if let Some(tot_cap) = total_tokens_regex().captures(&line) {
                    if let Some(t) = tot_cap.get(1).and_then(|m| parse_number(m.as_str())) {
                        total = Some(t);
                    }
                }

                let cost = cost_regex()
                    .captures(&line)
                    .and_then(|c| c.get(1))
                    .and_then(|m| m.as_str().parse::<f64>().ok());

                if inp.is_some() || out.is_some() || total.is_some() {
                    token_record = Some(TokenUsageRecord {
                        input_tokens: inp,
                        output_tokens: out,
                        cached_tokens: None,
                        total_tokens: total,
                        cost_usd: cost,
                    });
                }
            }
        }

        // 4. Try Context percentage: "Context window: 85% full"
        if let Some(caps) = context_percent_regex().captures(&line) {
            if let Some(pct_str) = caps.get(1) {
                if let Ok(pct) = pct_str.as_str().parse::<f64>() {
                    let ratio = pct / 100.0;
                    context_record = Some(ContextMetricsRecord {
                        context_fill_ratio: Some(ratio),
                        warning_triggered: ratio >= 0.80,
                    });
                }
            }
        } else if let Some(caps) = context_ratio_regex().captures(&line) {
            if let Some(rat_str) = caps.get(1) {
                if let Ok(ratio) = rat_str.as_str().parse::<f64>() {
                    context_record = Some(ContextMetricsRecord {
                        context_fill_ratio: Some(ratio),
                        warning_triggered: ratio >= 0.80,
                    });
                }
            }
        }

        (token_record, context_record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_token_scraping() {
        let line = "Tokens: 1,420 input, 380 output. Cost: $0.024";
        let (tokens, ctx) = TokenScraper::parse_line(line);
        assert!(ctx.is_none());
        assert!(tokens.is_some());
        let t = tokens.unwrap();
        assert_eq!(t.input_tokens, Some(1420));
        assert_eq!(t.output_tokens, Some(380));
        assert_eq!(t.total_tokens, Some(1800));
        assert_eq!(t.cost_usd, Some(0.024));
    }

    #[test]
    fn test_aider_token_scraping() {
        let line = "Tokens: 2.3k sent, 450 received. Cost: $0.012";
        let (tokens, _) = TokenScraper::parse_line(line);
        assert!(tokens.is_some());
        let t = tokens.unwrap();
        assert_eq!(t.input_tokens, Some(2300));
        assert_eq!(t.output_tokens, Some(450));
        assert_eq!(t.total_tokens, Some(2750));
        assert_eq!(t.cost_usd, Some(0.012));
    }

    #[test]
    fn test_prompt_completion_format() {
        let line = "Prompt tokens: 1,200, Completion tokens: 300, Total: 1,500";
        let (tokens, _) = TokenScraper::parse_line(line);
        assert!(tokens.is_some());
        let t = tokens.unwrap();
        assert_eq!(t.input_tokens, Some(1200));
        assert_eq!(t.output_tokens, Some(300));
        assert_eq!(t.total_tokens, Some(1500));
    }

    #[test]
    fn test_json_token_format() {
        let line = r#"{"prompt_tokens": 500, "completion_tokens": 120, "cost_usd": 0.005}"#;
        let (tokens, _) = TokenScraper::parse_line(line);
        assert!(tokens.is_some());
        let t = tokens.unwrap();
        assert_eq!(t.input_tokens, Some(500));
        assert_eq!(t.output_tokens, Some(120));
        assert_eq!(t.total_tokens, Some(620));
        assert_eq!(t.cost_usd, Some(0.005));
    }

    #[test]
    fn test_context_window_warning_trigger() {
        let line_safe = "Context window: 65% full";
        let (_, ctx_safe) = TokenScraper::parse_line(line_safe);
        assert!(ctx_safe.is_some());
        let cs = ctx_safe.unwrap();
        assert!((cs.context_fill_ratio.unwrap() - 0.65).abs() < 1e-4);
        assert!(!cs.warning_triggered);

        let line_warn = "\x1b[33mwarning: context window at 85% capacity\x1b[0m";
        let (_, ctx_warn) = TokenScraper::parse_line(line_warn);
        assert!(ctx_warn.is_some());
        let cw = ctx_warn.unwrap();
        assert!((cw.context_fill_ratio.unwrap() - 0.85).abs() < 1e-4);
        assert!(cw.warning_triggered);
    }
}
