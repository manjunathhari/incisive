//! Which look the pages wear (`LOOK`; no Leptos). Built for the browser too:
//! the page reads its look back from `<html data-look>`.

/// The page look: `Ivory` (today's, the default) or `Stipple`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Look {
    #[default]
    Ivory,
    Stipple,
}

impl Look {
    /// The `LOOK` value: `ivory` or `stipple`; anything else is `None`.
    pub fn parse(value: &str) -> Option<Look> {
        match value {
            "ivory" => Some(Look::Ivory),
            "stipple" => Some(Look::Stipple),
            _ => None,
        }
    }

    /// The `data-look` attribute on `<html>`: none for ivory (the page is
    /// exactly as before), `stipple` for stipple.
    pub fn attr(self) -> Option<&'static str> {
        match self {
            Look::Ivory => None,
            Look::Stipple => Some("stipple"),
        }
    }

    /// The look for a `data-look` value read back from the page: absent or
    /// unknown is ivory.
    pub fn from_attr(value: Option<&str>) -> Look {
        match value {
            Some("stipple") => Look::Stipple,
            _ => Look::Ivory,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The attribute written by the server reads back as the same look.
    #[test]
    fn attr_round_trips() {
        for look in [Look::Ivory, Look::Stipple] {
            assert_eq!(Look::from_attr(look.attr()), look);
        }
        assert_eq!(Look::Ivory.attr(), None);
        assert_eq!(Look::Stipple.attr(), Some("stipple"));
        assert_eq!(Look::from_attr(Some("dark")), Look::Ivory);
        assert_eq!(Look::parse("ivory"), Some(Look::Ivory));
        assert_eq!(Look::parse("stipple"), Some(Look::Stipple));
        assert_eq!(Look::parse("Stipple"), None);
    }
}
