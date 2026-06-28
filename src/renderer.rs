//! Tolerant `{{variable}}` substitution engine.
//!
//! The grammar is deliberately tiny: a placeholder is `{{` followed by one or
//! more of `[a-zA-Z0-9_]`, followed by `}}`. Rendering is a single linear pass
//! over the input (`O(N)` in the length of the template). Any placeholder whose
//! key is absent from the supplied variables is left **verbatim** — this is the
//! "tolerant rendering" guarantee the Data Plane depends on.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{Captures, Regex, Replacer};

/// Compiled once, reused for the lifetime of the process.
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{([a-zA-Z0-9_]+)\}\}").expect("placeholder regex is valid"));

/// A [`Replacer`] that writes substituted values directly into the output
/// buffer via [`Replacer::replace_append`], avoiding the intermediate `String`
/// that the closure form of `Replacer` must return per match.
struct DictReplacer<'r, 'v>(&'r HashMap<Cow<'v, str>, Cow<'v, str>>);

impl Replacer for DictReplacer<'_, '_> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        let key = &caps[1];
        match self.0.get(key) {
            Some(value) => dst.push_str(value.as_ref()),
            // Leave the original `{{key}}` literally in place.
            None => dst.push_str(&caps[0]),
        }
    }
}

/// A [`Replacer`] that parses the query string lazily on the first placeholder
/// match. Placeholder-free templates therefore scan once and never pay query
/// parsing, while templates with placeholders still scan only once.
struct QueryReplacer<'q> {
    query: &'q str,
    variables: Option<HashMap<Cow<'q, str>, Cow<'q, str>>>,
}

impl<'q> QueryReplacer<'q> {
    fn new(query: &'q str) -> Self {
        Self {
            query,
            variables: None,
        }
    }

    fn variables(&mut self) -> &HashMap<Cow<'q, str>, Cow<'q, str>> {
        if self.variables.is_none() {
            self.variables = Some(form_urlencoded::parse(self.query.as_bytes()).collect());
        }
        self.variables.as_ref().expect("variables just initialized")
    }
}

impl Replacer for QueryReplacer<'_> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        let key = &caps[1];
        match self.variables().get(key) {
            Some(value) => dst.push_str(value.as_ref()),
            None => dst.push_str(&caps[0]),
        }
    }
}

/// Render `template`, replacing every `{{key}}` whose `key` is present in
/// `variables` with the corresponding value. Unmatched placeholders are emitted
/// unchanged as literal text.
///
/// Runs in `O(N)` over the template length: `Regex::replace_all` performs a
/// single scan and never backtracks for this anchored, finite grammar.
///
/// Returns `Cow::Borrowed(template)` when no substitution occurs, avoiding any
/// heap allocation. The caller may then serve the original bytes directly (e.g.
/// via `bytes::Bytes::from_owner`) rather than copying.
#[must_use]
pub fn render<'a>(
    template: &'a str,
    variables: &HashMap<Cow<'_, str>, Cow<'_, str>>,
) -> Cow<'a, str> {
    PLACEHOLDER.replace_all(template, DictReplacer(variables))
}

/// Render `template` against the raw query string, parsing the query lazily only
/// if the renderer sees a placeholder.
#[must_use]
pub fn render_query<'a>(template: &'a str, query: &str) -> Cow<'a, str> {
    PLACEHOLDER.replace_all(template, QueryReplacer::new(query))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(pairs: &[(&'a str, &'a str)]) -> HashMap<Cow<'a, str>, Cow<'a, str>> {
        pairs
            .iter()
            .map(|(k, v)| (Cow::Borrowed(*k), Cow::Borrowed(*v)))
            .collect()
    }

    #[test]
    fn substitutes_known_keys() {
        let out = render("port={{port}}", &vars(&[("port", "8080")]));
        assert_eq!(out, "port=8080");
    }

    #[test]
    fn leaves_unknown_keys_literal() {
        // Acceptance criterion: tolerant rendering.
        let out = render("{{uuid}} on {{port}}", &vars(&[("port", "8080")]));
        assert_eq!(out, "{{uuid}} on 8080");
    }

    #[test]
    fn empty_variables_leaves_template_intact() {
        let out = render("{{a}}{{b}}", &vars(&[]));
        assert_eq!(out, "{{a}}{{b}}");
    }

    #[test]
    fn ignores_malformed_placeholders() {
        // Hyphen is not part of the key grammar, so this is not a placeholder.
        let out = render("{{ not-a-key }}", &vars(&[("not", "x")]));
        assert_eq!(out, "{{ not-a-key }}");
    }

    #[test]
    fn handles_adjacent_and_repeated_keys() {
        let out = render("{{x}}{{x}}-{{y}}", &vars(&[("x", "1"), ("y", "2")]));
        assert_eq!(out, "11-2");
    }

    #[test]
    fn passes_through_text_without_placeholders() {
        let out = render("plain text, no braces", &vars(&[("x", "1")]));
        assert_eq!(out, "plain text, no braces");
    }

    #[test]
    fn render_query_decodes_and_dedupes() {
        let out = render_query(
            "{{name}} on {{port}}",
            "port=8080&name=hello%20world&port=9090",
        );
        assert_eq!(out, "hello world on 9090");
    }

    #[test]
    fn render_query_leaves_unknown_keys_literal() {
        let out = render_query("{{uuid}} on {{port}}", "port=8080");
        assert_eq!(out, "{{uuid}} on 8080");
    }
}
