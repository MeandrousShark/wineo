//! The `[view.list_columns]` table: widths the user dragged the list view's
//! columns to. Only columns off their default width are written, and a bad
//! value counts as unset for that column only, like the rest of
//! `library.toml`.

use std::collections::BTreeMap;

use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

use super::{edit, store_value, view_table_mut};
use crate::error::Result;
use crate::paths::Paths;

/// The narrowest and widest a list column can be, in logical pixels.
pub const LIST_COLUMN_MIN: u16 = 56;
pub const LIST_COLUMN_MAX: u16 = 480;

/// A list view column that can be resized. Title is the leftover and has no
/// width of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ListColumn {
    State,
    Playtime,
    LastPlayed,
    Source,
    Tags,
}

impl ListColumn {
    /// Left to right.
    pub const ALL: [Self; 5] = [
        Self::State,
        Self::Playtime,
        Self::LastPlayed,
        Self::Source,
        Self::Tags,
    ];

    /// The key `[view.list_columns]` holds.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Playtime => "playtime",
            Self::LastPlayed => "last_played",
            Self::Source => "source",
            Self::Tags => "tags",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|column| column.as_str() == value)
    }

    /// The width the column has until it's resized.
    pub const fn default_width(self) -> u16 {
        match self {
            Self::State => 128,
            Self::Playtime => 96,
            Self::LastPlayed => 112,
            Self::Source => 80,
            Self::Tags => 176,
        }
    }
}

/// A saved width, or None for a value that isn't an integer in range.
pub(super) fn read_width(item: &Item) -> Option<u16> {
    item.as_integer()
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| (LIST_COLUMN_MIN..=LIST_COLUMN_MAX).contains(value))
}

/// Each key on its own: an unknown column or a bad width costs only itself.
pub(super) fn read_list_columns(item: Option<&Item>) -> BTreeMap<ListColumn, u16> {
    let Some(table) = item.and_then(Item::as_table_like) else {
        return BTreeMap::new();
    };
    table
        .iter()
        .filter_map(|(name, value)| Some((ListColumn::parse(name)?, read_width(value)?)))
        .collect()
}

/// Saves one column's width, or puts it back to its default with `None`. A
/// width outside the bounds is clamped, so a caller never saves a value the
/// reader would ignore. The default width is the same as none, which keeps
/// the table to the columns that were moved, and an emptied table's header
/// goes too. A bad value already in the file reads as the default, so
/// putting that column back to its default leaves the file alone.
pub fn set_list_column(paths: &Paths, column: ListColumn, width: Option<u16>) -> Result<bool> {
    let changed_only = |width: Option<u16>| width.filter(|width| *width != column.default_width());
    let wanted = changed_only(width.map(|width| width.clamp(LIST_COLUMN_MIN, LIST_COLUMN_MAX)));
    edit(paths, |document| {
        let have = read_list_columns(
            document
                .get("view")
                .and_then(Item::as_table_like)
                .and_then(|table| table.get("list_columns")),
        );
        if wanted == changed_only(have.get(&column).copied()) {
            return false;
        }
        match wanted {
            Some(width) => store_value(
                list_columns_table_mut(document),
                column.as_str(),
                Value::from(i64::from(width)),
            ),
            None => {
                list_columns_table_mut(document).remove(column.as_str());
                // Nothing left to say, so the header goes too.
                let view = view_table_mut(document);
                if view
                    .get("list_columns")
                    .and_then(Item::as_table_like)
                    .is_some_and(|columns| columns.is_empty())
                {
                    view.remove("list_columns");
                }
            }
        }
        true
    })
}

fn list_columns_table_mut(document: &mut DocumentMut) -> &mut dyn TableLike {
    let view = view_table_mut(document);
    if view
        .get("list_columns")
        .is_none_or(|item| !item.is_table_like())
    {
        view.insert("list_columns", Item::Table(Table::new()));
    }
    view.get_mut("list_columns")
        .and_then(Item::as_table_like_mut)
        .expect("list_columns was just made a table")
}
