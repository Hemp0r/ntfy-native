use crate::emoji_table::EMOJI;

pub fn lookup(alias: &str) -> Option<&'static str> {
    EMOJI
        .binary_search_by(|(a, _)| (*a).cmp(alias))
        .ok()
        .map(|i| EMOJI[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_lookup() {
        assert_eq!(lookup("warning"), Some("⚠️"));
        assert_eq!(lookup("+1"), Some("👍"));
        assert_eq!(lookup("zzz_unknown"), None);
    }

    #[test]
    fn emoji_table_sorted() {
        assert!(EMOJI.windows(2).all(|w| w[0].0 < w[1].0));
    }
}
