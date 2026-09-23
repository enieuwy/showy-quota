// Provider identity is data shared with the shell renderers. The payload from
// CodexBar still owns live status and provider discovery.
const REGISTRY: &str = include_str!("../../../share/providers.tsv");

fn entries() -> impl Iterator<Item = (&'static str, &'static str, &'static str, &'static str)> {
    REGISTRY.lines().filter_map(|line| {
        if line.starts_with('#') || line.is_empty() {
            return None;
        }
        let mut columns = line.split('\t');
        Some((
            columns.next()?,
            columns.next()?,
            columns.next()?,
            columns.next()?,
        ))
    })
}

pub(crate) fn sigil(provider: &str) -> Option<&'static str> {
    entries()
        .find(|(id, _, _, _)| *id == provider)
        .map(|(_, sigil, _, _)| sigil)
}

pub(crate) fn default_order() -> Vec<String> {
    entries()
        .filter(|(_, _, rank, _)| *rank != "-")
        .map(|(id, _, _, _)| id.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn registry_is_well_formed_and_legacy_collision_is_explicit() {
        let mut ids = HashSet::new();
        let mut sigils = HashMap::<_, Vec<_>>::new();
        let mut ranks = Vec::new();
        for (id, sigil, rank, font) in entries() {
            assert!(ids.insert(id), "duplicate provider id: {id}");
            assert!(
                id.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
                "invalid id: {id}"
            );
            assert_eq!(sigil.chars().count(), 2, "sigil must be two glyphs: {id}");
            sigils.entry(sigil).or_default().push(id);
            if rank != "-" {
                ranks.push(rank.parse::<usize>().expect("numeric rank"));
            }
            assert!(
                font == "-" || (font.starts_with(':') && font.ends_with(':')),
                "invalid font icon: {id}"
            );
        }
        assert_eq!(ranks, (1..=ranks.len()).collect::<Vec<_>>());
        let collisions: Vec<_> = sigils
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .collect();
        assert_eq!(collisions, vec![("FA", vec!["factory", "droid"])]);
        assert_eq!(
            default_order(),
            ["codex", "claude", "copilot", "opencode", "gemini"]
        );
    }
}
