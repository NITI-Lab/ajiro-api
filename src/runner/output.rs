//! Human-readable and JUnit XML output.

use crate::report::{TestReport, TestResult, TestStatus};
use serde_json::Value;
use std::fmt::Write;

fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{}...", t)
    }
}

/// `notes` are `(result index, text)` pairs, e.g. "snapshot written".
pub fn human(report: &TestReport, verbose: bool, notes: &[(usize, String)]) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "Environment: {} ({})", report.env, report.base_url);
    for le in &report.load_errors {
        let _ = writeln!(o, "! cannot load {}: {}", le.file, le.message);
    }
    for (i, r) in report.results.iter().enumerate() {
        let mark = match r.status {
            TestStatus::Passed => "✓",
            TestStatus::Failed => "✗",
            TestStatus::Skipped => "-",
            TestStatus::Error => "!",
        };
        let _ = write!(o, "{} {} / {}", mark, r.endpoint_id, r.test);
        if r.status != TestStatus::Skipped {
            let _ = write!(o, " ({}ms)", r.duration_ms);
        }
        let _ = writeln!(o);
        if r.status == TestStatus::Skipped {
            if let Some(reason) = &r.error {
                let _ = writeln!(o, "    skipped: {}", reason);
            }
        }
        if r.status == TestStatus::Error {
            let _ = writeln!(
                o,
                "    error: {}",
                r.error.as_deref().unwrap_or("unknown error")
            );
        }
        for f in &r.failures {
            let _ = writeln!(o, "    {}: {}", f.check, f.message);
            if let Some(p) = &f.path {
                let _ = writeln!(o, "      path:     {}", p);
            }
            let _ = writeln!(o, "      expected: {}", truncate(&show(&f.expected), 300));
            let _ = writeln!(o, "      actual:   {}", truncate(&show(&f.actual), 300));
        }
        for w in &r.warnings {
            let _ = writeln!(o, "    warning: {}", w);
        }
        if matches!(r.status, TestStatus::Failed | TestStatus::Error) {
            let _ = writeln!(o, "    → {}", r.file);
            if let Some(s) = &r.source {
                let _ = writeln!(o, "    → handler: {}:{}", s.file, s.line);
            }
        }
        for (idx, n) in notes {
            if *idx == i {
                let _ = writeln!(o, "    {}", n);
            }
        }
        if verbose {
            verbose_block(&mut o, r);
        }
    }
    let s = &report.summary;
    if s.total == 0 && report.load_errors.is_empty() {
        let _ = writeln!(o, "No tests matched.");
    }
    let _ = writeln!(
        o,
        "\n{} passed, {} failed, {} errors, {} skipped ({} total) in {}ms",
        s.passed, s.failed, s.errors, s.skipped, s.total, s.duration_ms
    );
    if s.warnings > 0 {
        let _ = writeln!(
            o,
            "{} passed with warnings (see `warning:` above); they do not fail the run",
            s.warnings
        );
    }
    o
}

fn verbose_block(o: &mut String, r: &TestResult) {
    if let Some(req) = &r.request {
        let _ = writeln!(o, "    > {} {}", req.method, req.url);
        for (k, v) in &req.headers {
            let _ = writeln!(o, "    > {}: {}", k, v);
        }
        if let Some(b) = &req.body {
            let _ = writeln!(o, "    > {}", truncate(&b.to_string(), 2000));
        }
    }
    if let Some(resp) = &r.response {
        let _ = writeln!(o, "    < {}", resp.status);
        for (k, v) in &resp.headers {
            let _ = writeln!(o, "    < {}: {}", k, v);
        }
        if let Some(b) = &resp.body {
            let _ = writeln!(o, "    < {}", truncate(&show(b), 2000));
        }
    }
}

// ----------------------------------------------------------------- junit ---

pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

fn secs(ms: u64) -> String {
    format!("{:.3}", ms as f64 / 1000.0)
}

pub fn junit(report: &TestReport) -> String {
    // Group by file, preserving first-seen order.
    let mut groups: Vec<(&str, Vec<&TestResult>)> = Vec::new();
    for r in &report.results {
        match groups.iter_mut().find(|(f, _)| *f == r.file.as_str()) {
            Some((_, v)) => v.push(r),
            None => groups.push((r.file.as_str(), vec![r])),
        }
    }
    let s = &report.summary;
    let mut o = String::new();
    let _ = writeln!(o, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        o,
        r#"<testsuites name="ajiro-api" tests="{}" failures="{}" errors="{}" skipped="{}" time="{}">"#,
        s.total,
        s.failed,
        s.errors,
        s.skipped,
        secs(s.duration_ms)
    );
    for (file, rs) in &groups {
        let failures = rs.iter().filter(|r| r.status == TestStatus::Failed).count();
        let errors = rs.iter().filter(|r| r.status == TestStatus::Error).count();
        let skipped = rs
            .iter()
            .filter(|r| r.status == TestStatus::Skipped)
            .count();
        let time: u64 = rs.iter().map(|r| r.duration_ms).sum();
        let _ = writeln!(
            o,
            r#"  <testsuite name="{}" tests="{}" failures="{}" errors="{}" skipped="{}" time="{}">"#,
            xml_escape(file),
            rs.len(),
            failures,
            errors,
            skipped,
            secs(time)
        );
        for r in rs {
            let _ = write!(
                o,
                r#"    <testcase classname="{}" name="{}" file="{}" time="{}""#,
                xml_escape(&r.endpoint_id),
                xml_escape(&r.test),
                xml_escape(&r.file),
                secs(r.duration_ms)
            );
            match r.status {
                TestStatus::Passed if r.warnings.is_empty() => {
                    let _ = writeln!(o, "/>");
                }
                TestStatus::Passed => {
                    let _ = writeln!(o, ">");
                    let text: Vec<String> = r
                        .warnings
                        .iter()
                        .map(|w| format!("warning: {}", w))
                        .collect();
                    let _ = writeln!(
                        o,
                        "      <system-out>{}</system-out>",
                        xml_escape(&text.join("\n"))
                    );
                    let _ = writeln!(o, "    </testcase>");
                }
                TestStatus::Skipped => {
                    let _ = writeln!(o, ">");
                    let _ = writeln!(
                        o,
                        r#"      <skipped message="{}"/>"#,
                        xml_escape(r.error.as_deref().unwrap_or("skipped"))
                    );
                    let _ = writeln!(o, "    </testcase>");
                }
                TestStatus::Error => {
                    let msg = r.error.as_deref().unwrap_or("error");
                    let _ = writeln!(o, ">");
                    let _ = writeln!(
                        o,
                        r#"      <error message="{}">{}</error>"#,
                        xml_escape(msg),
                        xml_escape(msg)
                    );
                    let _ = writeln!(o, "    </testcase>");
                }
                TestStatus::Failed => {
                    let _ = writeln!(o, ">");
                    for f in &r.failures {
                        let mut body = format!(
                            "check: {}\nmessage: {}\nexpected: {}\nactual: {}\n",
                            f.check,
                            f.message,
                            truncate(&show(&f.expected), 1000),
                            truncate(&show(&f.actual), 1000)
                        );
                        if let Some(p) = &f.path {
                            body.push_str(&format!("path: {}\n", p));
                        }
                        body.push_str(&format!("file: {}\n", r.file));
                        if let Some(src) = &r.source {
                            body.push_str(&format!("handler: {}:{}\n", src.file, src.line));
                        }
                        let _ = writeln!(
                            o,
                            r#"      <failure type="{}" message="{}">{}</failure>"#,
                            xml_escape(&f.check),
                            xml_escape(&f.message),
                            xml_escape(&body)
                        );
                    }
                    let _ = writeln!(o, "    </testcase>");
                }
            }
        }
        let _ = writeln!(o, "  </testsuite>");
    }
    let _ = writeln!(o, "</testsuites>");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        assert_eq!(xml_escape("a<b>&\"'\u{1}"), "a&lt;b&gt;&amp;&quot;&apos;");
    }

    #[test]
    fn warnings_are_printed_and_kept_in_junit() {
        let result = TestResult {
            endpoint_id: "a.me".into(),
            file: "api/a/get-me.yaml".into(),
            test: "smoke".into(),
            test_index: 0,
            status: TestStatus::Passed,
            duration_ms: 1,
            request: None,
            response: None,
            failures: vec![],
            error: None,
            source: None,
            warnings: vec!["smoke check passed with status 401: <login>".into()],
        };
        let report = TestReport {
            env: "local".into(),
            ok: true,
            summary: crate::report::TestSummary {
                total: 1,
                passed: 1,
                warnings: 1,
                ..Default::default()
            },
            results: vec![result],
            ..Default::default()
        };
        let h = human(&report, false, &[]);
        assert!(
            h.contains("✓ a.me / smoke (1ms)\n    warning: smoke check passed with status 401"),
            "{}",
            h
        );
        assert!(h.contains("1 passed with warnings"), "{}", h);
        let x = junit(&report);
        assert!(x.contains("<system-out>warning: smoke check passed with status 401: &lt;login&gt;</system-out>"), "{}", x);
        assert!(x.contains("</testcase>"), "{}", x);
    }
}
