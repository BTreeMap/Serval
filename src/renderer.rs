//! Tolerant `{{variable}}` substitution engine.
//!
//! The grammar is deliberately tiny: a placeholder is `{{` followed by one or
//! more of `[a-zA-Z0-9_]`, followed by `}}`. A deterministic scanner recognizes
//! placeholders in `O(N)` worst-case time and `O(1)` auxiliary scanner state,
//! with no backtracking. Any placeholder whose key is absent from the supplied
//! variables is left **verbatim** — this is the "tolerant rendering" guarantee
//! the Data Plane depends on.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use memchr::memmem::Finder;

/// SIMD-accelerated searcher for the grammar's only fixed delimiter.
static OPENER: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"{{"));

trait Resolver {
    fn resolve<'a>(&'a mut self, key: &str) -> Option<&'a str>;
}

struct DictResolver<'r, 'v>(&'r HashMap<Cow<'v, str>, Cow<'v, str>>);

impl Resolver for DictResolver<'_, '_> {
    fn resolve<'a>(&'a mut self, key: &str) -> Option<&'a str> {
        self.0.get(key).map(AsRef::as_ref)
    }
}

/// Parses the query lazily on the first syntactically valid placeholder.
struct QueryResolver<'q> {
    query: &'q str,
    variables: Option<HashMap<Cow<'q, str>, Cow<'q, str>>>,
}

impl<'q> QueryResolver<'q> {
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

impl Resolver for QueryResolver<'_> {
    fn resolve<'a>(&'a mut self, key: &str) -> Option<&'a str> {
        self.variables().get(key).map(AsRef::as_ref)
    }
}

#[inline]
fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Scan `template` once, allocating only after the first actual substitution.
///
/// `cursor` never moves backwards. [`Finder`] skips delimiter-free spans with
/// architecture-optimized substring search. A failed candidate advances past
/// its first `{`, allowing an overlapping opener such as the second byte of
/// `{{{key}}` to match. Search ranges never overlap except for that one-byte
/// fallback, and bytes inspected while validating a failed key are crossed at
/// most once more by the finder. Recognition therefore remains worst-case
/// `O(N)` even for malformed input. String slices are formed only at ASCII
/// delimiter/key boundaries, which are always UTF-8 boundaries.
fn render_with<'t>(template: &'t str, mut resolver: impl Resolver) -> Cow<'t, str> {
    let bytes = template.as_bytes();
    let mut cursor = 0;
    let mut copied_until = 0;
    let mut output: Option<String> = None;

    while let Some(relative_opener) = OPENER.find(&bytes[cursor..]) {
        let opener = cursor + relative_opener;
        let key_start = opener + 2;
        let mut key_end = key_start;
        while key_end < bytes.len() && is_key_byte(bytes[key_end]) {
            key_end += 1;
        }

        let valid = key_end > key_start
            && key_end + 1 < bytes.len()
            && bytes[key_end] == b'}'
            && bytes[key_end + 1] == b'}';
        if !valid {
            cursor = opener + 1;
            continue;
        }

        let placeholder_end = key_end + 2;
        let key = &template[key_start..key_end];
        if let Some(value) = resolver.resolve(key) {
            let rendered = output.get_or_insert_with(|| String::with_capacity(template.len()));
            rendered.push_str(&template[copied_until..opener]);
            rendered.push_str(value);
            copied_until = placeholder_end;
        }
        cursor = placeholder_end;
    }

    match output {
        Some(mut rendered) => {
            rendered.push_str(&template[copied_until..]);
            Cow::Owned(rendered)
        }
        None => Cow::Borrowed(template),
    }
}

/// Render `template`, replacing every `{{key}}` whose `key` is present in
/// `variables` with the corresponding value. Unmatched placeholders are emitted
/// unchanged as literal text.
///
/// Placeholder recognition is worst-case `O(N)` in the template length. With
/// standard expected `HashMap` behavior, complete rendering is expected
/// `O(N + O)`, where `O` is the emitted output length: hashing and comparing a
/// key costs `O(key.len())`, but valid placeholders are disjoint, so their
/// total key bytes are bounded by `N`. Adversarial hash collisions are outside
/// this expected bound.
///
/// Returns `Cow::Borrowed(template)` when no substitution occurs, avoiding any
/// heap allocation. The caller may then serve the original bytes directly (e.g.
/// via `bytes::Bytes::from_owner`) rather than copying.
#[must_use]
pub fn render<'a>(
    template: &'a str,
    variables: &HashMap<Cow<'_, str>, Cow<'_, str>>,
) -> Cow<'a, str> {
    render_with(template, DictResolver(variables))
}

/// Render `template` against the raw query string, parsing the query lazily only
/// if the scanner sees a syntactically valid placeholder.
///
/// With query length `Q` and output length `O`, complete rendering is expected
/// `O(N + Q + O)` under standard expected `HashMap` behavior. This includes
/// hashing and equality over all query and placeholder key bytes; adversarial
/// collisions can exceed the expected bound. The lexical scan itself retains a
/// deterministic `O(N)` bound.
#[must_use]
pub fn render_query<'a>(template: &'a str, query: &str) -> Cow<'a, str> {
    render_with(template, QueryResolver::new(query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow::{Borrowed, Owned};

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
        assert!(matches!(out, Borrowed(_)));
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
        assert!(matches!(out, Borrowed(_)));
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

    #[test]
    fn ownership_tracks_actual_substitution() {
        let variables = vars(&[("known", "value")]);
        for template in [
            "plain",
            "{{unknown}}",
            "{{}}",
            "{{unclosed",
            "{{bad-key}}",
            "{{ space }}",
            "{{café}}",
        ] {
            assert!(
                matches!(render(template, &variables), Borrowed(_)),
                "expected borrowed output for {template:?}"
            );
        }
        assert!(matches!(render("{{known}}", &variables), Owned(_)));
    }

    #[test]
    fn preserves_malformed_and_overlapping_braces() {
        let variables = vars(&[("x", "1")]);
        let cases = [
            ("", ""),
            ("{", "{"),
            ("{{", "{{"),
            ("{{}}", "{{}}"),
            ("{{x", "{{x"),
            ("{{x}", "{{x}"),
            ("{{bad-key}}", "{{bad-key}}"),
            ("{{{x}}", "{1"),
            ("{{{{x}}", "{{1"),
            ("{{x}}}", "1}"),
            ("{{x}}{{x}}", "11"),
        ];
        for (template, expected) in cases {
            assert_eq!(render(template, &variables), expected, "input {template:?}");
        }
    }

    #[test]
    fn accepts_every_key_character_and_preserves_unicode_text() {
        let variables = vars(&[("_Az09_", "✓")]);
        assert_eq!(render("π={{_Az09_}} 世界", &variables), "π=✓ 世界");
    }

    #[test]
    fn mixed_unknown_and_known_placeholders_preserve_literal_spans() {
        let variables = vars(&[("x", "1"), ("y", "2")]);
        assert_eq!(
            render("a{{missing}}b{{x}}c{{unknown}}d{{y}}e", &variables),
            "a{{missing}}b1c{{unknown}}d2e"
        );
    }

    #[test]
    fn query_renderer_handles_empty_and_encoded_values() {
        assert_eq!(
            render_query("{{path}}/{{empty}}/{{missing}}", "path=a%2Fb&empty="),
            "a/b//{{missing}}"
        );
        assert!(matches!(render_query("{{missing}}", ""), Borrowed(_)));
    }

    #[test]
    fn long_malformed_input_is_preserved_without_allocation() {
        let template = format!("{{{{{}-not-a-key}}}}", "a".repeat(100_000));
        let out = render(&template, &vars(&[("a", "x")]));
        assert_eq!(out, template);
        assert!(matches!(out, Borrowed(_)));
    }
}
