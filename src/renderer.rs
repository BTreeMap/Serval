//! Tolerant `{{variable}}` substitution engine.
//!
//! The grammar is deliberately tiny: a placeholder is `{{` followed by one or
//! more of `[a-zA-Z0-9_]`, followed by `}}`. Rendering is a single linear pass
//! over the input (`O(N)` in the length of the template). Any placeholder whose
//! key is absent from the supplied variables is left **verbatim** — this is the
//! "tolerant rendering" guarantee the Data Plane depends on.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Captures, Regex};

/// Compiled once, reused for the lifetime of the process.
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{([a-zA-Z0-9_]+)\}\}").expect("placeholder regex is valid"));

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
pub fn render<'a>(template: &'a str, variables: &[(Cow<'_, str>, Cow<'_, str>)]) -> Cow<'a, str> {
    PLACEHOLDER.replace_all(template, |caps: &Captures<'_>| {
        let key = &caps[1];
        match variables.iter().find(|(k, _)| k.as_ref() == key) {
            Some((_, value)) => value.as_ref().to_owned(),
            // Leave the original `{{key}}` literally in place.
            None => caps[0].to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(pairs: &[(&'a str, &'a str)]) -> Vec<(Cow<'a, str>, Cow<'a, str>)> {
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
}
