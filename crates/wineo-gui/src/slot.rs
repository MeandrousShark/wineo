//! Which drawn card is which. A game can be drawn in more than one place
//! once the library is grouped, so everything that belongs to a drawn card,
//! like its motion, hover, star pop, and widget id, keys by its slot: the
//! section it's drawn in plus the game's name. The selection and the marks
//! stay with the game.

use iced::advanced::widget::Id;

/// The section a slot is drawn in. The flat library is one section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(crate) enum SectionKey {
    /// The whole shown library, ungrouped.
    #[default]
    All,
    /// A second section for tests, so one game can sit in two slots.
    #[cfg(test)]
    Test(&'static str),
}

/// One drawn card: the game `name` in `section`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SlotId {
    section: SectionKey,
    name: String,
}

impl SlotId {
    pub(crate) fn new(section: SectionKey, name: impl Into<String>) -> Self {
        Self {
            section,
            name: name.into(),
        }
    }

    /// The game drawn here.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The card's container id, which scrolling to a card looks for. The
    /// flat library's keeps the id cards had before sections existed.
    pub(crate) fn widget_id(&self) -> Id {
        match &self.section {
            SectionKey::All => Id::from(format!("library-card:{}", self.name)),
            #[cfg(test)]
            SectionKey::Test(section) => Id::from(format!("library-card:{section}:{}", self.name)),
        }
    }
}

#[cfg(test)]
impl From<&str> for SlotId {
    fn from(name: &str) -> Self {
        Self::new(SectionKey::All, name)
    }
}

#[cfg(test)]
impl From<String> for SlotId {
    fn from(name: String) -> Self {
        Self::new(SectionKey::All, name)
    }
}

#[cfg(test)]
impl From<&String> for SlotId {
    fn from(name: &String) -> Self {
        Self::new(SectionKey::All, name.as_str())
    }
}

/// A plain name in a test stands for the flat library's slot, so any
/// other section's slot never equals it.
#[cfg(test)]
impl PartialEq<str> for SlotId {
    fn eq(&self, name: &str) -> bool {
        self.section == SectionKey::All && self.name == name
    }
}

#[cfg(test)]
impl PartialEq<&str> for SlotId {
    fn eq(&self, name: &&str) -> bool {
        *self == **name
    }
}

#[cfg(test)]
impl PartialEq<String> for SlotId {
    fn eq(&self, name: &String) -> bool {
        *self == *name.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flat_librarys_widget_id_is_the_one_cards_always_had() {
        let slot = SlotId::new(SectionKey::All, "hades");
        assert_eq!(slot.widget_id(), Id::from("library-card:hades".to_owned()));
        assert_eq!(SectionKey::default(), SectionKey::All);
    }

    #[test]
    fn two_sections_make_two_slots_for_one_game() {
        let flat = SlotId::new(SectionKey::All, "hades");
        let other = SlotId::new(SectionKey::Test("rpg"), "hades");
        assert_ne!(flat, other);
        assert_eq!(flat.name(), other.name());
        assert_ne!(flat.widget_id(), other.widget_id());
    }
}
