//! Bundled mission templates — the 10-template baseline corpus.
//!
//! Each template is one missionable prompt + one verification command +
//! one expected-shape hint. Kept in code (not on disk) so the bench can
//! run inside `cargo install`'ed binaries without a corpus download.
//!
//! Template set chosen per `docs/import_sweep_2026_05_19.md` §2.4:
//! storefront / arcade / portfolio / restaurant / dashboard
//! / csv-analytics / log-parser / rms-scheduler / dns-parser
//! / markdown-converter. The first five are HTML/static-site templates
//! that exercise structure + content; the next five exercise
//! algorithm + parsing on small Python / shell tasks.
//!
//! Curated from clawForge / stealthsambaV2 / overnight-run-3 corpora —
//! verbatim where solo-authored, paraphrased where Hadar-touched
//! (per `import_sweep_2026_05_19.md` §5 / [[project-import-sweep-2026-05-19]]).

use serde::{Deserialize, Serialize};

/// A single bench template. Drives one mission through the forge
/// pipeline; the validation command decides pass/fail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    /// Short kebab-case identifier (`csv-analytics`).
    pub name: &'static str,
    /// Human-readable one-line summary printed by `bench templates --list`.
    pub summary: &'static str,
    /// Verbatim mission prompt fed to the forge planner.
    pub mission: &'static str,
    /// Shell command run after the forge submitter. Exit code 0 ⇒ pass;
    /// any non-zero ⇒ fail. Run inside the mission workspace dir.
    pub validation: &'static str,
    /// Family — `static` (HTML/CSS), `algorithm`, `parsing`, `pipeline`.
    /// Used to group results in the summary view.
    pub family: TemplateFamily,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemplateFamily {
    Static,
    Algorithm,
    Parsing,
    Pipeline,
}

impl TemplateFamily {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Algorithm => "algorithm",
            Self::Parsing => "parsing",
            Self::Pipeline => "pipeline",
        }
    }
}

/// The 10-template baseline corpus. Returned as a borrowed slice so
/// callers can filter / iterate without cloning.
#[must_use]
pub fn all() -> &'static [Template] {
    TEMPLATES
}

/// Look up one template by name. Used by `bench run --template <name>`.
#[must_use]
pub fn by_name(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.name == name)
}

static TEMPLATES: &[Template] = &[
    Template {
        name: "storefront",
        summary: "Single-file HTML storefront with three product cards.",
        mission: "Build a single-file index.html storefront with three product cards (image / \
                  title / price / 'Add to Cart' button). Inline CSS. No JS required.",
        validation: "python3 -c \"import sys; html=open('index.html').read(); \
                     assert 'Add to Cart' in html and html.count('product') >= 3; print('OK')\"",
        family: TemplateFamily::Static,
    },
    Template {
        name: "arcade",
        summary: "Browser arcade landing page with hero + game grid.",
        mission: "Build a single-file index.html landing page for a retro arcade. Hero with title \
                  + tagline, a grid of 6 games (name + emoji + 1-line description). Inline CSS.",
        validation: "python3 -c \"html=open('index.html').read(); \
                     assert html.count('<h2') >= 6 or html.count('<h3') >= 6; print('OK')\"",
        family: TemplateFamily::Static,
    },
    Template {
        name: "portfolio",
        summary: "Personal portfolio: intro, three projects, contact.",
        mission: "Build a single-file index.html personal portfolio. Sections: intro / 3 \
                  projects (title + 2-line description) / contact (email link). Inline CSS.",
        validation: "python3 -c \"html=open('index.html').read(); \
                     assert 'mailto:' in html and html.count('project') >= 3; print('OK')\"",
        family: TemplateFamily::Static,
    },
    Template {
        name: "restaurant",
        summary: "Restaurant homepage with menu and hours.",
        mission: "Build a single-file index.html for a restaurant. Sections: hero (name + tagline) \
                  / menu (5+ items with prices) / hours (7-day grid). Inline CSS.",
        validation: "python3 -c \"html=open('index.html').read(); \
                     assert html.count('$') >= 5 and 'hours' in html.lower(); print('OK')\"",
        family: TemplateFamily::Static,
    },
    Template {
        name: "dashboard",
        summary: "Single-file admin dashboard mock with KPI tiles.",
        mission: "Build a single-file index.html admin dashboard mockup. Top bar + 4 KPI tiles \
                  + one data table (5 rows). Inline CSS, no real data fetches.",
        validation: "python3 -c \"html=open('index.html').read(); \
                     assert html.count('<table') >= 1 and html.count('KPI') + html.count('kpi') >= 1; \
                     print('OK')\"",
        family: TemplateFamily::Static,
    },
    Template {
        name: "csv-analytics",
        summary: "Parse a CSV and emit per-column min / max / mean.",
        mission: "Write a Python script `tasks/csv_stats.py` that reads `tasks/input.csv` and \
                  prints one line per numeric column in the form 'col=<name> min=<n> max=<n> \
                  mean=<n>'. Assume the first row is a header.",
        validation: "echo 'a,b,c\\n1,2,3\\n4,5,6\\n7,8,9' > tasks/input.csv && \
                     python3 tasks/csv_stats.py | grep -q 'col=a min=1 max=7 mean=4'",
        family: TemplateFamily::Parsing,
    },
    Template {
        name: "log-parser",
        summary: "Apache-style access-log parser → counts by status code.",
        mission: "Write a Python script `tasks/log_count.py` that reads `tasks/access.log` (one \
                  Apache common-format line per row), counts requests by HTTP status code, and \
                  prints '<code> <count>' lines sorted by code ascending.",
        validation: "printf '%s\\n' '1.2.3.4 - - [01/Jan/2026:00:00:00 +0000] \"GET / HTTP/1.1\" 200 1024' \
                     '1.2.3.4 - - [01/Jan/2026:00:00:01 +0000] \"GET /a HTTP/1.1\" 404 -' \
                     '1.2.3.4 - - [01/Jan/2026:00:00:02 +0000] \"GET / HTTP/1.1\" 200 1024' > tasks/access.log && \
                     python3 tasks/log_count.py | grep -q '200 2' && python3 tasks/log_count.py | grep -q '404 1'",
        family: TemplateFamily::Parsing,
    },
    Template {
        name: "rms-scheduler",
        summary: "Rate-monotonic scheduler — sort tasks by period asc.",
        mission: "Write a Python script `tasks/rms.py` that reads a JSON file `tasks/tasks.json` \
                  containing a list of `{name, period, exec_time}` task records and prints them \
                  sorted by `period` ascending in the form '<name> period=<n>'. Periods are \
                  integer milliseconds.",
        validation: "echo '[{\"name\":\"A\",\"period\":50,\"exec_time\":10},{\"name\":\"B\",\"period\":20,\"exec_time\":5}]' \
                     > tasks/tasks.json && python3 tasks/rms.py | head -1 | grep -q 'B period=20'",
        family: TemplateFamily::Algorithm,
    },
    Template {
        name: "dns-parser",
        summary: "Read /etc/hosts-style file → emit (hostname, ip) lines.",
        mission: "Write a Python script `tasks/hosts.py` that reads `tasks/hosts.txt` (one line \
                  per record, comments start with '#', whitespace-separated `ip hostname [aliases…]`) \
                  and prints '<hostname> <ip>' lines, one per primary hostname, ignoring comments \
                  and blank lines.",
        validation: "printf '%s\\n' '# comment' '127.0.0.1 localhost' '' '10.0.0.1 router lan-gw' > tasks/hosts.txt && \
                     python3 tasks/hosts.py | grep -q 'localhost 127.0.0.1' && \
                     python3 tasks/hosts.py | grep -q 'router 10.0.0.1'",
        family: TemplateFamily::Parsing,
    },
    Template {
        name: "markdown-converter",
        summary: "Tiny markdown → HTML for headings + paragraphs.",
        mission: "Write a Python script `tasks/md2html.py` that reads `tasks/input.md` and prints \
                  HTML to stdout. Support: `# h1` / `## h2` / `### h3` headings and paragraphs \
                  (blank-line-separated). No code fences, no lists — those are out of scope.",
        validation: "printf '%s\\n' '# Hi' '' 'A para.' > tasks/input.md && \
                     python3 tasks/md2html.py | grep -q '<h1>Hi</h1>' && \
                     python3 tasks/md2html.py | grep -q '<p>A para.</p>'",
        family: TemplateFamily::Pipeline,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_templates_bundled() {
        assert_eq!(all().len(), 10, "baseline corpus expects exactly 10");
    }

    #[test]
    fn every_template_has_unique_name() {
        let mut names: Vec<&str> = all().iter().map(|t| t.name).collect();
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total, "template names must be unique");
    }

    #[test]
    fn every_template_has_a_non_empty_mission_and_validation() {
        for t in all() {
            assert!(!t.mission.is_empty(), "{}: empty mission", t.name);
            assert!(!t.validation.is_empty(), "{}: empty validation", t.name);
            assert!(!t.summary.is_empty(), "{}: empty summary", t.name);
        }
    }

    #[test]
    fn by_name_finds_known_template() {
        let t = by_name("csv-analytics").expect("csv-analytics is bundled");
        assert_eq!(t.family, TemplateFamily::Parsing);
    }

    #[test]
    fn by_name_returns_none_for_unknown() {
        assert!(by_name("does-not-exist").is_none());
    }

    #[test]
    fn families_partition_the_corpus() {
        // Sanity: every template assigns to one of the four families.
        for t in all() {
            let s = t.family.as_str();
            assert!(matches!(s, "static" | "algorithm" | "parsing" | "pipeline"));
        }
    }
}
