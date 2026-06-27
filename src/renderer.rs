//! Tolerant `{{variable}}` substitution engine.
//!
//! The grammar is deliberately tiny: a placeholder is `{{` followed by one or
//! more of `[a-zA-Z0-9_]`, followed by `}}`. Rendering is a single linear pass
//! over the input (`O(N)` in the length of the template). Any placeholder whose
//! key is absent from the supplied variables is left **verbatim** — this is the
//! "tolerant rendering" guarantee the Data Plane depends on.

use std::collections::HashMap;
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
#[must_use]
pub fn render(template: &str, variables: &HashMap<String, String>) -> String {
    PLACEHOLDER
        .replace_all(template, |caps: &Captures<'_>| {
            let key = &caps[1];
            match variables.get(key) {
                Some(value) => value.clone(),
                // Leave the original `{{key}}` literally in place.
                None => caps[0].to_string(),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
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
