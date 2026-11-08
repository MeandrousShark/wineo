//! Which drawn card is which. A game can be drawn in more than one place
//! once the library is grouped, so everything that belongs to a drawn card,
//! like its motion, hover, star pop, and widget id, keys by its slot: the
//! section it's drawn in plus the game's name. The selection and the marks
//! stay with the game.

use iced::advanced::widget::Id;

/// The section a slot is drawn in. The flat library is one section; a
/// grouped one has a section per tag, source, or state, with Recently
/// played and Hidden pinned around them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum SectionKey {
    /// The whole shown library: ungrouped, with no header, or under Group
    /// by Recently played, as a section with one. The two never show at
    /// once, so they share slot and widget ids and a switch between them
    /// moves each card instead of fading it.
    #[default]
    All,
    /// The last ten games played, pinned first.
    Recent,
    Tag(String),
    /// Games with no tags, after every tag.
    NoTag,
    /// A store, else a platform, by its label.
    Source(&'static str),
    /// Games with neither, after every source.
    NoSource,
    Running,
    /// Everything that's neither running nor not installed.
    OtherState,
    NotInstalled,
    /// Hidden games while Show hidden is on, pinned last.
    Hidden,
    /// A second section for tests, so one game can sit in two slots.
    #[cfg(test)]
    Test(&'static str),
}

impl SectionKey {
    /// The section's name in its header.
    pub(crate) fn title(&self) -> String {
        match self {
            Self::All => "All".to_owned(),
            Self::Recent => "Recently played".to_owned(),
            Self::Tag(tag) => tag.clone(),
            Self::NoTag => "No tag".to_owned(),
            Self::Source(label) => (*label).to_owned(),
            Self::NoSource => "No source".to_owned(),
            Self::Running => "Running".to_owned(),
            Self::OtherState => "Other".to_owned(),
            Self::NotInstalled => "Not installed".to_owned(),
            Self::Hidden => "Hidden".to_owned(),
            #[cfg(test)]
            Self::Test(section) => (*section).to_owned(),
        }
    }

    /// The section header's id, which Tab and scrolling reach.
    pub(crate) fn header_id(&self) -> Id {
        Id::from(format!("library-section:{}", self.id()))
    }

    /// The section's part of an id. Names and tags never hold a colon, so
    /// it can't run into the name after it.
    fn id(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Recent => "recent".to_owned(),
            Self::Tag(tag) => format!("tag:{tag}"),
            Self::NoTag => "no-tag".to_owned(),
            Self::Source(label) => format!("source:{label}"),
            Self::NoSource => "no-source".to_owned(),
            Self::Running => "state:running".to_owned(),
            Self::OtherState => "state:other".to_owned(),
            Self::NotInstalled => "state:not-installed".to_owned(),
            Self::Hidden => "hidden".to_owned(),
            #[cfg(test)]
            Self::Test(section) => (*section).to_owned(),
        }
    }
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

    /// The section it's drawn in.
    pub(crate) fn section(&self) -> &SectionKey {
        &self.section
    }

    /// The card's container id, which scrolling to a card looks for. The
    /// flat library's keeps the id cards had before sections existed.
    pub(crate) fn widget_id(&self) -> Id {
        match &self.section {
            SectionKey::All => Id::from(format!("library-card:{}", self.name)),
            section => Id::from(format!("library-card:{}:{}", section.id(), self.name)),
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

    #[test]
    fn every_section_gives_its_copies_and_header_their_own_ids() {
        let sections = [
            SectionKey::All,
            SectionKey::Recent,
            SectionKey::Tag("rpg".to_owned()),
            SectionKey::Tag("no-tag".to_owned()),
            SectionKey::NoTag,
            SectionKey::Source("Steam"),
            SectionKey::NoSource,
            SectionKey::Running,
            SectionKey::OtherState,
            SectionKey::NotInstalled,
            SectionKey::Hidden,
        ];
        let cards: std::collections::HashSet<Id> = sections
            .iter()
            .map(|section| SlotId::new(section.clone(), "hades").widget_id())
            .collect();
        assert_eq!(cards.len(), sections.len());
        let headers: std::collections::HashSet<Id> =
            sections.iter().map(SectionKey::header_id).collect();
        assert_eq!(headers.len(), sections.len());
        assert_eq!(
            SlotId::new(SectionKey::Tag("rpg".to_owned()), "hades").widget_id(),
            Id::from("library-card:tag:rpg:hades".to_owned())
        );
        assert_eq!(
            SectionKey::Recent.header_id(),
            Id::from("library-section:recent".to_owned())
        );
    }
}
