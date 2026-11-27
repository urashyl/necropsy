// SPDX-License-Identifier: AGPL-3.0-or-later
//! Filling `{{NAME}}` placeholders in a file kept in the generator's source.

/// Replaces each `{{name}}` with its value. Panics on a placeholder that has no
/// value, so a typo fails the build's check instead of reaching a player.
pub fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let j = after
            .find("}}")
            .unwrap_or_else(|| panic!("unclosed {{{{ in template"));
        let name = &after[..j];
        let value = values
            .iter()
            .find(|(k, _)| *k == name)
            .unwrap_or_else(|| panic!("no value for {{{{{name}}}}}"))
            .1;
        out.push_str(value);
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_every_placeholder() {
        assert_eq!(
            render("a {{X}} b {{Y}}{{X}}", &[("X", "1"), ("Y", "2")]),
            "a 1 b 21"
        );
        assert_eq!(render("plain", &[]), "plain");
    }

    #[test]
    #[should_panic(expected = "no value for {{Z}}")]
    fn a_missing_value_panics() {
        render("{{Z}}", &[]);
    }
}
