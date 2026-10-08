//! Side-by-side event/marker probe: yaml-rust2 0.11.1 vs saphyr-parser 0.1.0.
//!
//! For each named input, prints one row per event with the yaml-rust2 start
//! marker and the saphyr start/end markers (all in the libraries' own units,
//! which are *chars*), plus the saphyr span's source text (sliced by chars).
//! A `!` in the first column flags rows where the two start indices differ.

use std::fmt::Write as _;

mod cases;

#[derive(Debug, Clone)]
struct Ev {
    kind: String,
    start: (usize, usize, usize),
    end: Option<(usize, usize, usize)>,
}

fn yr2_events(src: &str) -> (Vec<Ev>, Option<String>) {
    use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
    use yaml_rust2::scanner::Marker;
    struct R(Vec<Ev>);
    impl MarkedEventReceiver for R {
        fn on_event(&mut self, ev: Event, m: Marker) {
            let kind = match &ev {
                Event::Scalar(v, style, aid, tag) => format!(
                    "Scalar({v:?}, {style:?}, a{aid}, {})",
                    tag.as_ref().map(|t| format!("{}|{}", t.handle, t.suffix)).unwrap_or_default()
                ),
                Event::SequenceStart(aid, tag) => format!(
                    "SeqStart(a{aid}, {})",
                    tag.as_ref().map(|t| format!("{}|{}", t.handle, t.suffix)).unwrap_or_default()
                ),
                Event::MappingStart(aid, tag) => format!(
                    "MapStart(a{aid}, {})",
                    tag.as_ref().map(|t| format!("{}|{}", t.handle, t.suffix)).unwrap_or_default()
                ),
                other => format!("{other:?}"),
            };
            self.0.push(Ev { kind, start: (m.index(), m.line(), m.col()), end: None });
        }
    }
    let mut r = R(Vec::new());
    let err = Parser::new_from_str(src)
        .load(&mut r, true)
        .err()
        .map(|e| format!("{:?} @ ({},{},{})", e.info(), e.marker().index(), e.marker().line(), e.marker().col()));
    (r.0, err)
}

fn saphyr_events(src: &str) -> (Vec<Ev>, Option<String>) {
    use saphyr_parser::{Event, Parser, Span, SpannedEventReceiver};
    struct R(Vec<Ev>);
    impl<'i> SpannedEventReceiver<'i> for R {
        fn on_event(&mut self, ev: Event<'i>, s: Span) {
            let kind = match &ev {
                Event::Scalar(v, style, aid, tag) => format!(
                    "Scalar({v:?}, {style:?}, a{aid}, {})",
                    tag.as_ref().map(|t| format!("{}|{}", t.handle, t.suffix)).unwrap_or_default()
                ),
                Event::SequenceStart(aid, tag) => format!(
                    "SeqStart(a{aid}, {})",
                    tag.as_ref().map(|t| format!("{}|{}", t.handle, t.suffix)).unwrap_or_default()
                ),
                Event::MappingStart(aid, tag) => format!(
                    "MapStart(a{aid}, {})",
                    tag.as_ref().map(|t| format!("{}|{}", t.handle, t.suffix)).unwrap_or_default()
                ),
                Event::DocumentStart(explicit) => format!("DocumentStart({explicit})"),
                other => format!("{other:?}"),
            };
            self.0.push(Ev {
                kind,
                start: (s.start.index(), s.start.line(), s.start.col()),
                end: Some((s.end.index(), s.end.line(), s.end.col())),
            });
        }
    }
    let mut r = R(Vec::new());
    let err = Parser::new_from_str(src)
        .load(&mut r, true)
        .err()
        .map(|e| format!("{:?} @ ({},{},{})", e.info(), e.marker().index(), e.marker().line(), e.marker().col()));
    (r.0, err)
}

fn char_slice(src: &str, start: usize, end: usize) -> String {
    src.chars().skip(start).take(end.saturating_sub(start)).collect()
}

fn run(name: &str, src: &str) {
    let (a, ea) = yr2_events(src);
    let (b, eb) = saphyr_events(src);
    let mut out = String::new();
    writeln!(out, "==== {name} ====").unwrap();
    writeln!(out, "{}", src.replace('\n', "⏎\n").replace('\t', "⇥")).unwrap();
    writeln!(out, "  src: {} bytes, {} chars, ascii={}", src.len(), src.chars().count(), src.is_ascii()).unwrap();
    writeln!(out, "{:<1} {:<14} {:<14} {:<14}  {:<50} {}", "", "yr2 start", "sap start", "sap end", "event (yr2 / sap if differs)", "sap span text").unwrap();
    let n = a.len().max(b.len());
    for i in 0..n {
        let ya = a.get(i);
        let sb = b.get(i);
        let fmt = |t: (usize, usize, usize)| format!("{}:{}:{}", t.0, t.1, t.2);
        let ys = ya.map(|e| fmt(e.start)).unwrap_or("-".into());
        let ss = sb.map(|e| fmt(e.start)).unwrap_or("-".into());
        let se = sb.and_then(|e| e.end).map(fmt).unwrap_or("-".into());
        let flag = match (ya, sb) {
            (Some(x), Some(y)) if x.start.0 != y.start.0 => "!",
            (Some(_), None) | (None, Some(_)) => "?",
            _ => " ",
        };
        let kind = match (ya, sb) {
            (Some(x), Some(y)) if x.kind == y.kind => x.kind.clone(),
            (Some(x), Some(y)) => format!("{} / {}", x.kind, y.kind),
            (Some(x), None) => format!("{} / -", x.kind),
            (None, Some(y)) => format!("- / {}", y.kind),
            _ => String::new(),
        };
        let text = sb
            .and_then(|e| e.end.map(|end| char_slice(src, e.start.0, end.0)))
            .map(|t| format!("{t:?}"))
            .unwrap_or_default();
        writeln!(out, "{flag:<1} {ys:<14} {ss:<14} {se:<14}  {kind:<50} {text}").unwrap();
    }
    if ea.is_some() || eb.is_some() {
        writeln!(out, "  yr2 error: {}", ea.unwrap_or("none".into())).unwrap();
        writeln!(out, "  sap error: {}", eb.unwrap_or("none".into())).unwrap();
    }
    println!("{out}");
}

fn main() {
    let cases = cases::cases();
    let only: Vec<String> = std::env::args().skip(1).collect();
    for (name, src) in &cases {
        if !only.is_empty() && !only.iter().any(|o| name.contains(o.as_str())) {
            continue;
        }
        run(name, src);
    }
}
